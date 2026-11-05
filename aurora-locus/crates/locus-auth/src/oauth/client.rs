//! OAuth clients, as the reference's `ClientManager` and `Client`: client id
//! forms (discoverable HTTPS metadata URLs and `http://localhost` loopback
//! clients), metadata parsing with the schema defaults, the validation
//! rules for metadata and redirect URIs, `private_key_jwt` authentication,
//! and request checks against the metadata.

use serde_json::{Map, Value, json};
use url::Url;

use super::OAuthError;
use super::jose::{Jws, PublicJwk, VERIFY_ALGS, thumbprint};

const MINUTE_MS: i64 = 60_000;
const DAY_MS: i64 = 24 * 60 * MINUTE_MS;
const YEAR_MS: i64 = 365 * DAY_MS + DAY_MS / 4;

pub const SESSION_LIFETIME_MS: i64 = 14 * DAY_MS;
pub const REFRESH_LIFETIME_MS: i64 = 14 * DAY_MS;
pub const SESSION_LIFETIME_EXTENDED_MS: i64 = 2 * YEAR_MS;
pub const REFRESH_LIFETIME_EXTENDED_MS: i64 = YEAR_MS / 4;
pub const CLIENT_ASSERTION_MAX_AGE_S: i64 = 60;
pub const CLIENT_ASSERTION_TYPE_JWT_BEARER: &str =
    "urn:ietf:params:oauth:client-assertion-type:jwt-bearer";

pub const DEFAULT_LOOPBACK_REDIRECT_URIS: &[&str] = &["http://127.0.0.1/", "http://[::1]/"];

fn bad_metadata(m: impl Into<String>) -> OAuthError {
    OAuthError::invalid_client_metadata(m)
}

fn bad_redirect(m: impl Into<String>) -> OAuthError {
    OAuthError::new("invalid_redirect_uri", m, 400)
}

fn bad_client_id(m: impl Into<String>) -> OAuthError {
    OAuthError::new("invalid_client_id", m, 400)
}

// ---------------------------------------------------------------- hosts

#[must_use]
pub fn is_hostname_ip(h: &str) -> bool {
    let v4 = h.split('.').count() == 4
        && h.split('.')
            .all(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()));
    v4 || (h.starts_with('[') && h.ends_with(']'))
}

#[must_use]
pub fn is_loopback_host(h: &str) -> bool {
    matches!(h, "localhost" | "127.0.0.1" | "[::1]")
}

/// The reference's `isLocalHostname`: one label, or a reserved TLD.
#[must_use]
pub fn is_local_hostname(h: &str) -> bool {
    let parts: Vec<&str> = h.split('.').collect();
    if parts.len() < 2 {
        return true;
    }
    matches!(
        parts.last().map(|t| t.to_ascii_lowercase()).as_deref(),
        Some("test" | "local" | "localhost" | "invalid" | "example")
    )
}

fn host(u: &Url) -> &str {
    u.host_str().unwrap_or("")
}

/// `http:` URL on a loopback host.
fn is_loopback_uri(v: &str) -> bool {
    v.starts_with("http://") && Url::parse(v).is_ok_and(|u| is_loopback_host(host(&u)))
}

/// An `https:` URL on a non-loopback host with a dotted, non-`.local` name.
fn https_uri_problem(v: &str) -> Option<&'static str> {
    if !v.starts_with("https://") {
        return Some("URL must use the \"https:\" protocol");
    }
    let Ok(u) = Url::parse(v) else {
        return Some("Invalid URL");
    };
    let h = host(&u);
    if is_loopback_host(h) {
        return Some("https: URL must not use a loopback host");
    }
    if !is_hostname_ip(h) {
        if !h.contains('.') {
            return Some("Domain name must contain at least two segments");
        }
        if h.ends_with(".local") {
            return Some("Domain name must not end with \".local\"");
        }
    }
    None
}

fn is_web_uri(v: &str) -> bool {
    if v.starts_with("http://") {
        is_loopback_uri(v)
    } else {
        https_uri_problem(v).is_none()
    }
}

/// `<reverse.domain>:/path`.
fn private_use_problem(v: &str) -> Option<String> {
    let (dot, colon) = (v.find('.'), v.find(':'));
    match (dot, colon) {
        (Some(d), Some(c)) if d < c => {}
        _ => return Some("Private-use URI scheme requires a \".\" as part of the protocol".into()),
    }
    let Ok(u) = Url::parse(v) else {
        return Some("Invalid URL".into());
    };
    let domain: Vec<&str> = u.scheme().split('.').rev().collect();
    if is_local_hostname(&domain.join(".")) {
        return Some("Private-use URI Scheme redirect URI must not be a local hostname".into());
    }
    if v.starts_with(&format!("{}://", u.scheme()))
        || !u.username().is_empty()
        || u.password().is_some()
        || u.host_str().is_some_and(|h| !h.is_empty())
        || u.port().is_some()
    {
        return Some(
            "Private-Use URI Scheme must be in the form <scheme>:/{path} (notice the single slash!) as per RFC 8252"
                .into(),
        );
    }
    None
}

/// The redirect URI schema check.
fn redirect_uri_problem(v: &str) -> Option<String> {
    if v.starts_with("https:") {
        https_uri_problem(v).map(str::to_owned)
    } else if v.starts_with("http:") {
        if !is_loopback_uri(v) {
            return Some(
                "URL must use \"localhost\", \"127.0.0.1\" or \"[::1]\" as hostname".into(),
            );
        }
        if v.starts_with("http://localhost") {
            return Some(
                "Use of \"localhost\" hostname is not allowed (RFC 8252), use a loopback IP such as \"127.0.0.1\" instead"
                    .into(),
            );
        }
        None
    } else if is_private_use_shape(v) {
        private_use_problem(v)
    } else {
        Some("URL must use the \"https:\" or \"http:\" protocol, or a private-use URI scheme (RFC 8252)".into())
    }
}

/// `^[^.:]+(?:\.[^.:]+)+:`
fn is_private_use_shape(v: &str) -> bool {
    let Some(c) = v.find(':') else { return false };
    let scheme = &v[..c];
    let parts: Vec<&str> = scheme.split('.').collect();
    parts.len() >= 2 && parts.iter().all(|p| !p.is_empty())
}

// ---------------------------------------------------------------- client ids

/// Whether `id` is a loopback client id (`http://localhost[/][?...]`).
pub fn parse_loopback_client_id(id: &str) -> Result<(Option<String>, Option<Vec<String>>), String> {
    let origin = "http://localhost";
    if !id.starts_with(origin) {
        return Err(format!("Value must start with \"{origin}\""));
    }
    if id[origin.len()..].contains('#') {
        return Err("Value must not contain a hash component".into());
    }
    let q = if id.as_bytes().get(origin.len()) == Some(&b'/') {
        origin.len() + 1
    } else {
        origin.len()
    };
    if id.len() != q && id.as_bytes().get(q) != Some(&b'?') {
        return Err("Value must not contain a path component".into());
    }
    let query = id.get(q + 1..).unwrap_or("");
    let (mut scope, mut redirects): (Option<String>, Option<Vec<String>>) = (None, None);
    for (k, v) in url::form_urlencoded::parse(query.as_bytes()) {
        match k.as_ref() {
            "scope" => {
                if scope.is_some() {
                    return Err("Duplicate \"scope\" query parameter".into());
                }
                if v.is_empty() {
                    return Err("Invalid \"scope\" query parameter: Validation failed".into());
                }
                scope = Some(v.into_owned());
            }
            "redirect_uri" => {
                if !is_loopback_uri(&v) || v.starts_with("http://localhost") {
                    return Err(
                        "Invalid \"redirect_uri\" query parameter: Validation failed".into(),
                    );
                }
                redirects.get_or_insert_with(Vec::new).push(v.into_owned());
            }
            other => return Err(format!("Unexpected query parameter \"{other}\"")),
        }
    }
    Ok((scope, redirects))
}

#[must_use]
pub fn is_loopback_client_id(id: &str) -> bool {
    parse_loopback_client_id(id).is_ok()
}

/// A discoverable client id: its metadata URL.
pub fn parse_discoverable_client_id(id: &str) -> Result<Url, String> {
    if let Some(p) = https_uri_problem(id) {
        return Err(p.to_owned());
    }
    let u = Url::parse(id).map_err(|_| "Invalid URL".to_owned())?;
    if !u.username().is_empty() || u.password().is_some() {
        return Err("ClientID must not contain credentials".into());
    }
    if u.fragment().is_some() {
        return Err("ClientID must not contain a fragment".into());
    }
    if u.path() == "/" {
        return Err(
            "ClientID must contain a path component (e.g. \"/client-metadata.json\")".into(),
        );
    }
    if u.path().ends_with('/') {
        return Err("ClientID path must not end with a trailing slash".into());
    }
    if is_hostname_ip(host(&u)) {
        return Err("ClientID hostname must not be an IP address".into());
    }
    // Canonical form: the path as written is the normalized one.
    let after_host = id
        .splitn(4, '/')
        .nth(3)
        .map(|r| format!("/{r}"))
        .unwrap_or_default();
    let written_path = after_host.split(['?', '#']).next().unwrap_or("");
    if written_path != u.path() {
        return Err(format!(
            "ClientID must be in canonical form (\"{u}\", got \"{id}\")"
        ));
    }
    Ok(u)
}

#[must_use]
pub fn is_discoverable_client_id(id: &str) -> bool {
    parse_discoverable_client_id(id).is_ok()
}

/// The metadata of a loopback client, as `atprotoLoopbackClientMetadata`.
pub fn loopback_metadata(id: &str) -> Result<Value, OAuthError> {
    let (scope, redirects) = parse_loopback_client_id(id)
        .map_err(|e| bad_metadata(format!("Invalid loopback client id \"{id}\": {e}")))?;
    let scope = scope.unwrap_or_else(|| "atproto".to_owned());
    if !scope.split(' ').any(|s| s == "atproto") {
        return Err(bad_metadata(format!(
            "Invalid loopback client id \"{id}\": ATProto Loopback ClientID must include \"atproto\" scope"
        )));
    }
    let redirects = redirects.unwrap_or_else(|| {
        DEFAULT_LOOPBACK_REDIRECT_URIS
            .iter()
            .map(|s| (*s).to_owned())
            .collect()
    });
    Ok(json!({
        "client_id": id,
        "scope": scope,
        "redirect_uris": redirects,
        "response_types": ["code"],
        "grant_types": ["authorization_code", "refresh_token"],
        "token_endpoint_auth_method": "none",
        "application_type": "native",
        "dpop_bound_access_tokens": true,
    }))
}

// ---------------------------------------------------------------- metadata

/// Client metadata with the schema's defaults applied.
#[derive(Debug, Clone, PartialEq)]
pub struct ClientMetadata {
    pub raw: Map<String, Value>,
    pub redirect_uris: Vec<String>,
    pub response_types: Vec<String>,
    pub grant_types: Vec<String>,
    pub scope: Option<String>,
    pub token_endpoint_auth_method: String,
    pub token_endpoint_auth_signing_alg: Option<String>,
    pub jwks_uri: Option<String>,
    pub jwks: Option<Value>,
    pub application_type: String,
    pub client_id: Option<String>,
    pub client_uri: Option<String>,
    pub request_object_signing_alg: Option<String>,
    pub dpop_bound_access_tokens: Option<bool>,
    pub authorization_details_types: Option<Vec<String>>,
}

const RESPONSE_TYPES: &[&str] = &[
    "code",
    "token",
    "none",
    "code id_token token",
    "code id_token",
    "code token",
    "id_token token",
    "id_token",
];
const GRANT_TYPES: &[&str] = &[
    "authorization_code",
    "implicit",
    "refresh_token",
    "password",
    "client_credentials",
    "urn:ietf:params:oauth:grant-type:jwt-bearer",
    "urn:ietf:params:oauth:grant-type:saml2-bearer",
];
const AUTH_METHODS: &[&str] = &[
    "client_secret_basic",
    "client_secret_jwt",
    "client_secret_post",
    "none",
    "private_key_jwt",
    "self_signed_tls_client_auth",
    "tls_client_auth",
];

fn str_list(
    m: &Map<String, Value>,
    k: &str,
    allowed: Option<&[&str]>,
) -> Result<Option<Vec<String>>, OAuthError> {
    match m.get(k) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Array(a)) => {
            let mut out = Vec::new();
            for v in a {
                let s = v
                    .as_str()
                    .ok_or_else(|| bad_metadata(format!("Invalid \"{k}\"")))?;
                if allowed.is_some_and(|al| !al.contains(&s)) {
                    return Err(bad_metadata(format!("Invalid \"{k}\" value \"{s}\"")));
                }
                out.push(s.to_owned());
            }
            if out.is_empty() {
                return Err(bad_metadata(format!("\"{k}\" must not be empty")));
            }
            Ok(Some(out))
        }
        Some(_) => Err(bad_metadata(format!("Invalid \"{k}\""))),
    }
}

fn opt_str(m: &Map<String, Value>, k: &str) -> Result<Option<String>, OAuthError> {
    match m.get(k) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(s)) => Ok(Some(s.clone())),
        Some(_) => Err(bad_metadata(format!("Invalid \"{k}\""))),
    }
}

impl ClientMetadata {
    /// Parses metadata as the reference's schema (defaults applied: code
    /// response type, authorization_code grant, client_secret_basic auth,
    /// web application).
    pub fn parse(v: &Value) -> Result<Self, OAuthError> {
        let m = v
            .as_object()
            .ok_or_else(|| bad_metadata("Client metadata must be an object"))?;
        let redirect_uris = str_list(m, "redirect_uris", None)?
            .ok_or_else(|| bad_metadata("At least one redirect_uri is required"))?;
        for r in &redirect_uris {
            if let Some(p) = redirect_uri_problem(r) {
                return Err(bad_metadata(format!("Invalid redirect_uri \"{r}\": {p}")));
            }
        }
        for k in [
            "jwks_uri",
            "client_uri",
            "policy_uri",
            "tos_uri",
            "logo_uri",
        ] {
            if let Some(u) = opt_str(m, k)?
                && !is_web_uri(&u)
            {
                return Err(bad_metadata(format!("Invalid \"{k}\"")));
            }
        }
        let token_endpoint_auth_method = opt_str(m, "token_endpoint_auth_method")?
            .unwrap_or_else(|| "client_secret_basic".into());
        if !AUTH_METHODS.contains(&token_endpoint_auth_method.as_str()) {
            return Err(bad_metadata("Invalid \"token_endpoint_auth_method\""));
        }
        let application_type = opt_str(m, "application_type")?.unwrap_or_else(|| "web".into());
        if application_type != "web" && application_type != "native" {
            return Err(bad_metadata("Invalid \"application_type\""));
        }
        let jwks = match m.get("jwks") {
            None | Some(Value::Null) => None,
            Some(j) => Some(public_jwks(j)?),
        };
        let dpop = match m.get("dpop_bound_access_tokens") {
            None | Some(Value::Null) => None,
            Some(Value::Bool(b)) => Some(*b),
            Some(_) => return Err(bad_metadata("Invalid \"dpop_bound_access_tokens\"")),
        };
        Ok(Self {
            raw: m.clone(),
            redirect_uris,
            response_types: str_list(m, "response_types", Some(RESPONSE_TYPES))?
                .unwrap_or_else(|| vec!["code".into()]),
            grant_types: str_list(m, "grant_types", Some(GRANT_TYPES))?
                .unwrap_or_else(|| vec!["authorization_code".into()]),
            scope: opt_str(m, "scope")?,
            token_endpoint_auth_method,
            token_endpoint_auth_signing_alg: opt_str(m, "token_endpoint_auth_signing_alg")?,
            jwks_uri: opt_str(m, "jwks_uri")?,
            jwks,
            application_type,
            client_id: opt_str(m, "client_id")?,
            client_uri: opt_str(m, "client_uri")?,
            request_object_signing_alg: opt_str(m, "request_object_signing_alg")?,
            dpop_bound_access_tokens: dpop,
            authorization_details_types: str_list(m, "authorization_details_types", None)?,
        })
    }

    /// `client_name`, for the consent page.
    #[must_use]
    pub fn client_name(&self) -> Option<&str> {
        self.raw.get("client_name").and_then(Value::as_str)
    }
}

/// A JWKS with only the public keys locus understands (others ignored,
/// as the reference's `jwksPubSchema`).
pub fn public_jwks(v: &Value) -> Result<Value, OAuthError> {
    let keys = v
        .get("keys")
        .and_then(Value::as_array)
        .ok_or_else(|| bad_metadata("Invalid \"jwks\""))?;
    let kept: Vec<Value> = keys
        .iter()
        .filter(|k| {
            k.as_object()
                .is_some_and(|m| PublicJwk::from_jwk(m).is_ok())
        })
        .cloned()
        .collect();
    Ok(json!({ "keys": kept }))
}

fn duplicate(items: &[String]) -> Option<&String> {
    items
        .iter()
        .enumerate()
        .find(|(i, v)| items[i + 1..].contains(v))
        .map(|(_, v)| v)
}

/// The reference's `validateClientMetadata` (with its loopback and
/// discoverable additions).
pub fn validate_metadata(client_id: &str, md: &ClientMetadata) -> Result<(), OAuthError> {
    if md.jwks.is_some() && md.jwks_uri.is_some() {
        return Err(bad_metadata("jwks_uri and jwks are mutually exclusive"));
    }
    for k in [
        "default_max_age",
        "userinfo_signed_response_alg",
        "id_token_signed_response_alg",
        "userinfo_encrypted_response_alg",
    ] {
        if md.raw.get(k).is_some_and(|v| !v.is_null()) {
            return Err(bad_metadata(format!("Unsupported \"{k}\" parameter")));
        }
    }
    if let Some(cu) = &md.client_uri
        && Url::parse(cu).is_ok_and(|u| is_local_hostname(host(&u)))
    {
        return Err(bad_metadata("client_uri hostname is invalid"));
    }
    let scopes: Vec<String> = md
        .scope
        .as_deref()
        .ok_or_else(|| bad_metadata("Missing scope property"))?
        .split(' ')
        .map(str::to_owned)
        .collect();
    if !scopes.iter().any(|s| s == "atproto") {
        return Err(bad_metadata("Missing \"atproto\" scope"));
    }
    if let Some(d) = duplicate(&scopes) {
        return Err(bad_metadata(format!("Duplicate scope \"{d}\"")));
    }
    if let Some(d) = duplicate(&md.grant_types) {
        return Err(bad_metadata(format!("Duplicate grant type \"{d}\"")));
    }
    for g in &md.grant_types {
        match g.as_str() {
            "implicit" => return Err(bad_metadata(format!("Grant type \"{g}\" is not allowed"))),
            "authorization_code" | "refresh_token" => {}
            _ => return Err(bad_metadata(format!("Grant type \"{g}\" is not supported"))),
        }
    }
    if md.client_id.as_deref().is_some_and(|c| c != client_id) {
        return Err(bad_metadata("client_id does not match"));
    }
    if md
        .raw
        .get("subject_type")
        .and_then(Value::as_str)
        .is_some_and(|t| t != "public")
    {
        return Err(bad_metadata("Only \"public\" subject_type is supported"));
    }
    match md.token_endpoint_auth_method.as_str() {
        "none" => {
            if md.token_endpoint_auth_signing_alg.is_some() {
                return Err(bad_metadata(
                    "token_endpoint_auth_method \"none\" must not have token_endpoint_auth_signing_alg",
                ));
            }
        }
        "private_key_jwt" => {
            if md.jwks.is_none() && md.jwks_uri.is_none() {
                return Err(bad_metadata(
                    "private_key_jwt auth method requires jwks or jwks_uri",
                ));
            }
            if md
                .jwks
                .as_ref()
                .is_some_and(|j| j["keys"].as_array().is_some_and(Vec::is_empty))
            {
                return Err(bad_metadata(
                    "private_key_jwt auth method requires at least one key in jwks",
                ));
            }
            if md.token_endpoint_auth_signing_alg.is_none() {
                return Err(bad_metadata(
                    "Missing token_endpoint_auth_signing_alg client metadata",
                ));
            }
        }
        other => {
            return Err(bad_metadata(format!(
                "Unsupported client authentication method \"{other}\". Make sure \"token_endpoint_auth_method\" is set to one of: \"none\", \"private_key_jwt\""
            )));
        }
    }
    if md
        .raw
        .get("authorization_encrypted_response_enc")
        .is_some_and(|v| !v.is_null())
    {
        return Err(bad_metadata(
            "Encrypted authorization response is not supported",
        ));
    }
    if md.raw.get("tls_client_certificate_bound_access_tokens") == Some(&Value::Bool(true)) {
        return Err(bad_metadata(
            "Mutual-TLS bound access tokens are not supported",
        ));
    }
    if md.dpop_bound_access_tokens != Some(true) {
        return Err(bad_metadata("\"dpop_bound_access_tokens\" must be true"));
    }
    if !md.response_types.iter().any(|r| r == "code") {
        return Err(bad_metadata("response_types must include \"code\""));
    }
    if !md.grant_types.iter().any(|g| g == "authorization_code") {
        return Err(bad_metadata(
            "The \"code\" response type requires that \"grant_types\" contains \"authorization_code\"",
        ));
    }
    if let Some(types) = &md.authorization_details_types
        && !types.is_empty()
    {
        if let Some(d) = duplicate(types) {
            return Err(bad_metadata(format!(
                "Duplicate authorization_details_type \"{d}\""
            )));
        }
        return Err(bad_metadata(
            "authorization_details_types are not supported",
        ));
    }
    if md.redirect_uris.is_empty() {
        return Err(bad_metadata("At least one redirect_uri is required"));
    }
    if md.application_type == "native" && md.token_endpoint_auth_method != "none" {
        return Err(bad_metadata(
            "Native clients must authenticate using \"none\" method",
        ));
    }
    for r in &md.redirect_uris {
        let u = Url::parse(r).map_err(|_| bad_redirect(format!("Invalid redirect URI {r}")))?;
        if !u.username().is_empty() || u.password().is_some() {
            return Err(bad_redirect(format!(
                "Redirect URI {u} must not contain credentials"
            )));
        }
        let h = host(&u);
        if h == "localhost" {
            return Err(bad_redirect(format!(
                "Loopback redirect URI {u} is not allowed (use explicit IPs instead)"
            )));
        }
        if h == "127.0.0.1" || h == "[::1]" {
            if md.application_type != "native" {
                return Err(bad_redirect(
                    "Loopback redirect URIs are only allowed for native apps",
                ));
            }
            if u.scheme() != "http" {
                return Err(bad_redirect(format!(
                    "Loopback redirect URI {u} must use HTTP"
                )));
            }
        } else if u.scheme() == "http" {
            return Err(bad_redirect(
                "Only loopback redirect URIs are allowed to use the \"http\" scheme",
            ));
        } else if u.scheme() == "https" {
            if is_local_hostname(h) {
                return Err(bad_redirect(format!(
                    "Redirect URI \"{u}\"'s domain name must not be a local hostname"
                )));
            }
        } else if u.scheme().contains('.') {
            if md.application_type != "native" {
                return Err(bad_redirect(
                    "Private-Use URI Scheme redirect URI are only allowed for native apps",
                ));
            }
        } else {
            return Err(bad_redirect(format!(
                "Invalid redirect URI scheme \"{}:\"",
                u.scheme()
            )));
        }
    }
    if is_loopback_client_id(client_id) {
        if md.client_uri.is_some() {
            return Err(bad_metadata(
                "client_uri is not allowed for loopback clients",
            ));
        }
        if md.application_type != "native" {
            return Err(bad_metadata(
                "Loopback clients must have application_type \"native\"",
            ));
        }
        if md.token_endpoint_auth_method != "none" {
            return Err(bad_metadata(format!(
                "Loopback clients are not allowed to use \"token_endpoint_auth_method\" {}",
                md.token_endpoint_auth_method
            )));
        }
    } else if let Ok(id_url) = parse_discoverable_client_id(client_id) {
        if md.client_id.is_none() {
            return Err(bad_metadata(
                "client_id is required for discoverable clients",
            ));
        }
        if let Some(cu) = &md.client_uri {
            let cu = Url::parse(cu).map_err(|_| bad_metadata("Invalid client_uri"))?;
            if cu.origin() != id_url.origin() {
                return Err(bad_metadata(
                    "client_uri must have the same origin as the client_id",
                ));
            }
            if id_url.path() != cu.path() {
                let parent = if cu.path().ends_with('/') {
                    cu.path().to_owned()
                } else {
                    format!("{}/", cu.path())
                };
                if !id_url.path().starts_with(&parent) {
                    return Err(bad_metadata(
                        "client_uri must be a parent URL of the client_id",
                    ));
                }
            }
        }
        for r in &md.redirect_uris {
            let u = Url::parse(r).map_err(|_| bad_redirect(format!("Invalid redirect URI {r}")))?;
            if u.scheme().contains('.') {
                let want: Vec<&str> = host(&id_url).split('.').rev().collect();
                let want = want.join(".");
                if u.scheme() != want {
                    return Err(bad_redirect(format!(
                        "Private-Use URI Scheme redirect URI, for discoverable client metadata, must be the fully qualified domain name (FQDN) of the client_id, in reverse order ({want}:)"
                    )));
                }
            }
        }
    }
    Ok(())
}

/// The metadata URL a client id is fetched from, with the id checks.
pub fn metadata_url(client_id: &str) -> Result<Url, OAuthError> {
    let u = parse_discoverable_client_id(client_id)
        .map_err(|e| bad_client_id(format!("Invalid discoverable client identifier: {e}")))?;
    if is_local_hostname(host(&u)) {
        return Err(bad_client_id(
            "The client_id's TLD must not be a local hostname",
        ));
    }
    Ok(u)
}

/// The reference's `compareRedirectUri` (any port for loopback URIs
/// registered without one, RFC 8252).
#[must_use]
pub fn redirect_uri_matches(allowed: &str, requested: &str) -> bool {
    if allowed == requested {
        return true;
    }
    let (Ok(a), Ok(r)) = (Url::parse(allowed), Url::parse(requested)) else {
        return false;
    };
    if !is_loopback_host(host(&a)) {
        return false;
    }
    (a.port().is_none() || a.port() == r.port())
        && host(&a) == host(&r)
        && a.path() == r.path()
        && a.scheme() == r.scheme()
        && a.query() == r.query()
        && a.fragment() == r.fragment()
        && a.username() == r.username()
        && a.password() == r.password()
}

// ---------------------------------------------------------------- the client

/// How a client authenticated at the token endpoint (stored with grants,
/// and must stay the same on refresh).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "method")]
pub enum ClientAuth {
    #[serde(rename = "none")]
    None,
    #[serde(rename = "private_key_jwt")]
    PrivateKeyJwt {
        alg: String,
        kid: String,
        jkt: String,
        jti: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        exp: Option<i64>,
    },
}

/// A resolved client.
#[derive(Debug, Clone)]
pub struct Client {
    pub id: String,
    pub metadata: ClientMetadata,
    /// Keys for `private_key_jwt` (inline or fetched from `jwks_uri`).
    pub jwks: Option<Value>,
    pub is_trusted: bool,
    pub is_first_party: bool,
}

/// What a `private_key_jwt` assertion carried (its `jti` is checked for
/// replay by the caller).
impl Client {
    #[must_use]
    pub fn is_confidential(&self) -> bool {
        self.metadata.token_endpoint_auth_method != "none"
    }

    fn extended(&self) -> bool {
        self.is_confidential() || (self.is_first_party && self.is_trusted)
    }

    #[must_use]
    pub fn refresh_lifetime_ms(&self) -> i64 {
        if self.extended() {
            REFRESH_LIFETIME_EXTENDED_MS
        } else {
            REFRESH_LIFETIME_MS
        }
    }

    #[must_use]
    pub fn session_lifetime_ms(&self) -> i64 {
        if self.extended() {
            SESSION_LIFETIME_EXTENDED_MS
        } else {
            SESSION_LIFETIME_MS
        }
    }

    #[must_use]
    pub fn default_redirect_uri(&self) -> Option<&str> {
        match self.metadata.redirect_uris.as_slice() {
            [one] => Some(one),
            _ => None,
        }
    }

    /// A JWT signed with one of the client's keys, issued by the client.
    /// Returns the header, payload and the key's thumbprint.
    fn verify_client_jwt(
        &self,
        token: &str,
        allowed_alg: Option<&str>,
    ) -> Result<(Jws, String), String> {
        let jws = Jws::decode(token).map_err(|e| e.0)?;
        let alg = jws.alg().to_owned();
        if !VERIFY_ALGS.contains(&alg.as_str()) || allowed_alg.is_some_and(|a| a != alg) {
            return Err("\"alg\" (Algorithm) Header Parameter value not allowed".into());
        }
        let kid = jws.header.get("kid").and_then(Value::as_str);
        let keys = self
            .jwks
            .as_ref()
            .and_then(|j| j["keys"].as_array().cloned())
            .unwrap_or_default();
        let candidates: Vec<&Map<String, Value>> = keys
            .iter()
            .filter_map(Value::as_object)
            .filter(|k| kid.is_none_or(|id| k.get("kid").and_then(Value::as_str) == Some(id)))
            .filter(|k| {
                k.get("alg")
                    .and_then(Value::as_str)
                    .is_none_or(|a| a == alg)
            })
            .filter(|k| {
                k.get("use")
                    .and_then(Value::as_str)
                    .is_none_or(|u| u == "sig")
            })
            .collect();
        if candidates.is_empty() {
            return Err("no applicable key found in the JSON Web Key Set".into());
        }
        for k in candidates {
            if let Ok(public) = PublicJwk::from_jwk(k)
                && public
                    .verify(&alg, &jws.signing_input, &jws.signature)
                    .is_ok()
            {
                let jkt = thumbprint(k).map_err(|e| e.0)?;
                if jws.payload.get("iss").and_then(Value::as_str) != Some(self.id.as_str()) {
                    return Err("unexpected \"iss\" claim value".into());
                }
                return Ok((jws, jkt));
            }
        }
        Err("signature verification failed".into())
    }

    /// Authenticates the client at the token endpoint, as the reference's
    /// `Client.authenticate`. `issuer` is the authorization server.
    pub fn authenticate(
        &self,
        assertion_type: Option<&str>,
        assertion: Option<&str>,
        issuer: &str,
        now_s: i64,
    ) -> Result<ClientAuth, OAuthError> {
        match self.metadata.token_endpoint_auth_method.as_str() {
            "none" => Ok(ClientAuth::None),
            "private_key_jwt" => {
                let Some(assertion) = assertion else {
                    return Err(OAuthError::invalid_request(
                        "client authentication method \"private_key_jwt\" required a \"client_assertion\"",
                    ));
                };
                if assertion_type != Some(CLIENT_ASSERTION_TYPE_JWT_BEARER) {
                    return Err(OAuthError::invalid_client(format!(
                        "Unsupported client_assertion_type \"{}\"",
                        assertion_type.unwrap_or("undefined")
                    )));
                }
                let failed = |m: &str| {
                    OAuthError::invalid_client(format!(
                        "Validation of \"client_assertion\" failed: {m}"
                    ))
                };
                let (jws, jkt) = self
                    .verify_client_jwt(assertion, None)
                    .map_err(|e| failed(&e))?;
                let p = &jws.payload;
                if p.get("sub").and_then(Value::as_str) != Some(self.id.as_str()) {
                    return Err(failed("unexpected \"sub\" claim value"));
                }
                let aud_ok = match p.get("aud") {
                    Some(Value::String(a)) => a == issuer,
                    Some(Value::Array(a)) => a.iter().any(|x| x.as_str() == Some(issuer)),
                    _ => false,
                };
                if !aud_ok {
                    return Err(failed("unexpected \"aud\" claim value"));
                }
                let jti = p
                    .get("jti")
                    .and_then(Value::as_str)
                    .ok_or_else(|| failed("missing required \"jti\" claim"))?
                    .to_owned();
                let iat = p
                    .get("iat")
                    .and_then(Value::as_i64)
                    .ok_or_else(|| failed("\"iat\" claim must be a number"))?;
                if now_s - iat > CLIENT_ASSERTION_MAX_AGE_S {
                    return Err(failed(
                        "\"iat\" claim timestamp check failed (too far in the past)",
                    ));
                }
                let exp = p.get("exp").and_then(Value::as_i64);
                if exp.is_some_and(|e| e <= now_s) {
                    return Err(failed("\"exp\" claim timestamp check failed"));
                }
                let kid = jws
                    .header
                    .get("kid")
                    .and_then(Value::as_str)
                    .ok_or_else(|| {
                        OAuthError::invalid_client("\"kid\" required in client_assertion")
                    })?
                    .to_owned();
                Ok(ClientAuth::PrivateKeyJwt {
                    alg: jws.alg().to_owned(),
                    kid,
                    jkt,
                    jti,
                    exp,
                })
            }
            other => Err(bad_metadata(format!(
                "Unsupported token_endpoint_auth_method \"{other}\""
            ))),
        }
    }

    /// A request object (JAR) from this client, as `decodeRequestObject`:
    /// its claims (unsigned only when the client registered `none`).
    pub fn decode_request_object(
        &self,
        jar: &str,
        audience: &str,
        now_s: i64,
    ) -> Result<Map<String, Value>, OAuthError> {
        let bad = |m: &str| OAuthError::invalid_request(format!("Invalid \"request\" object: {m}"));
        let payload = if self.metadata.request_object_signing_alg.as_deref() == Some("none") {
            let mut parts = jar.split('.');
            let (Some(h), Some(p)) = (parts.next(), parts.next()) else {
                return Err(bad("Invalid Unsecured JWT"));
            };
            let header: Value =
                serde_json::from_slice(&super::jose::unb64(h).map_err(|e| bad(&e.0))?)
                    .map_err(|_| bad("Invalid Unsecured JWT"))?;
            if header.get("alg").and_then(Value::as_str) != Some("none") {
                return Err(bad("Invalid Unsecured JWT"));
            }
            let payload: Value =
                serde_json::from_slice(&super::jose::unb64(p).map_err(|e| bad(&e.0))?)
                    .map_err(|_| bad("Invalid Unsecured JWT"))?;
            let p = payload
                .as_object()
                .cloned()
                .ok_or_else(|| bad("Invalid Unsecured JWT"))?;
            if p.get("iss")
                .is_some_and(|i| i.as_str() != Some(self.id.as_str()))
            {
                return Err(bad(&format!("Invalid \"iss\" claim \"{}\"", p["iss"])));
            }
            p
        } else {
            let (jws, _) = self
                .verify_client_jwt(jar, self.metadata.request_object_signing_alg.as_deref())
                .map_err(|e| bad(&e))?;
            jws.payload
        };
        if let Some(aud) = payload.get("aud") {
            let ok = match aud {
                Value::String(a) => a == audience,
                Value::Array(a) => a.iter().any(|x| x.as_str() == Some(audience)),
                _ => false,
            };
            if !ok {
                return Err(bad(&format!("Invalid \"aud\" claim \"{aud}\"")));
            }
        }
        let iat = payload
            .get("iat")
            .and_then(Value::as_i64)
            .ok_or_else(|| bad("\"iat\" claim must be a number"))?;
        if now_s - iat > 59 {
            return Err(bad(
                "\"iat\" claim timestamp check failed (too far in the past)",
            ));
        }
        Ok(payload)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn md(v: Value) -> ClientMetadata {
        ClientMetadata::parse(&v).unwrap()
    }

    fn web(id: &str) -> Value {
        json!({
            "client_id": id,
            "scope": "atproto transition:generic",
            "redirect_uris": ["https://app.example.com/callback"],
            "grant_types": ["authorization_code", "refresh_token"],
            "token_endpoint_auth_method": "none",
            "application_type": "web",
            "dpop_bound_access_tokens": true,
            "client_uri": "https://app.example.com",
        })
    }

    #[test]
    fn client_ids() {
        assert!(is_loopback_client_id("http://localhost"));
        assert!(is_loopback_client_id(
            "http://localhost/?scope=atproto&redirect_uri=http%3A%2F%2F127.0.0.1%3A8080%2Fcb"
        ));
        assert!(!is_loopback_client_id("http://localhost/path"));
        assert!(!is_loopback_client_id("http://localhost?foo=bar"));
        assert!(!is_loopback_client_id("http://localhost#x"));
        assert!(is_discoverable_client_id(
            "https://app.bsky.social/client-metadata.json"
        ));
        for bad in [
            "https://app.example.com/",
            "https://app.example.com/meta/",
            "https://1.2.3.4/meta.json",
            "https://app.example.com/a/../meta.json",
            "http://app.example.com/meta.json",
            "https://localhost/meta.json",
        ] {
            assert!(!is_discoverable_client_id(bad), "{bad}");
        }
        assert!(
            metadata_url("https://app.test/meta.json").is_err(),
            "local TLD"
        );
        let lb = loopback_metadata("http://localhost").unwrap();
        assert_eq!(
            lb["redirect_uris"],
            json!(["http://127.0.0.1/", "http://[::1]/"])
        );
        let m = md(lb);
        validate_metadata("http://localhost", &m).unwrap();
    }

    #[test]
    fn metadata_rules() {
        let id = "https://app.example.com/client-metadata.json";
        validate_metadata(id, &md(web(id))).unwrap();
        let fails = |mutate: &dyn Fn(&mut Value), want: &str| {
            let mut v = web(id);
            mutate(&mut v);
            let err = ClientMetadata::parse(&v)
                .and_then(|m| validate_metadata(id, &m))
                .unwrap_err();
            assert!(
                err.description.contains(want),
                "{want}: got {}",
                err.description
            );
        };
        fails(
            &|v| v["scope"] = json!("transition:generic"),
            "Missing \"atproto\" scope",
        );
        fails(
            &|v| v["scope"] = json!("atproto atproto"),
            "Duplicate scope",
        );
        fails(
            &|v| v["dpop_bound_access_tokens"] = json!(false),
            "dpop_bound_access_tokens",
        );
        fails(
            &|v| v["client_id"] = json!("https://other.example.com/m.json"),
            "client_id does not match",
        );
        fails(
            &|v| v["grant_types"] = json!(["implicit", "authorization_code"]),
            "is not allowed",
        );
        fails(
            &|v| v["token_endpoint_auth_method"] = json!("client_secret_basic"),
            "Unsupported client authentication method",
        );
        fails(
            &|v| {
                v.as_object_mut()
                    .unwrap()
                    .remove("token_endpoint_auth_method");
            },
            "Unsupported client authentication method",
        );
        fails(
            &|v| v["redirect_uris"] = json!(["http://127.0.0.1/cb"]),
            "only allowed for native apps",
        );
        fails(
            &|v| v["redirect_uris"] = json!(["https://app.test/cb"]),
            "must not be a local hostname",
        );
        fails(
            &|v| v["client_uri"] = json!("https://elsewhere.example.com"),
            "same origin",
        );
        fails(
            &|v| v["token_endpoint_auth_method"] = json!("private_key_jwt"),
            "requires jwks or jwks_uri",
        );
        // Native apps with private-use schemes named after the client id's host.
        let mut native = web(id);
        native["application_type"] = json!("native");
        native["redirect_uris"] = json!(["com.example.app:/callback", "http://127.0.0.1/cb"]);
        validate_metadata(id, &md(native.clone())).unwrap();
        native["redirect_uris"] = json!(["com.other.app:/callback"]);
        assert!(
            validate_metadata(id, &md(native))
                .unwrap_err()
                .description
                .contains("reverse order")
        );
    }

    #[test]
    fn redirect_matching() {
        assert!(redirect_uri_matches(
            "http://127.0.0.1/cb",
            "http://127.0.0.1:43210/cb"
        ));
        assert!(!redirect_uri_matches(
            "http://127.0.0.1:8000/cb",
            "http://127.0.0.1:43210/cb"
        ));
        assert!(!redirect_uri_matches(
            "http://127.0.0.1/cb",
            "http://127.0.0.1:43210/other"
        ));
        assert!(!redirect_uri_matches(
            "https://app.example.com/cb",
            "https://app.example.com/cb2"
        ));
        assert!(redirect_uri_matches(
            "https://app.example.com/cb",
            "https://app.example.com/cb"
        ));
    }

    #[test]
    fn private_key_jwt_auth() {
        use super::super::jose::Es256Key;
        let key = Es256Key::generate();
        let id = "https://app.example.com/client-metadata.json";
        let mut v = web(id);
        v["token_endpoint_auth_method"] = json!("private_key_jwt");
        v["token_endpoint_auth_signing_alg"] = json!("ES256");
        v["jwks"] = json!({"keys": [key.jwk()]});
        let m = md(v);
        validate_metadata(id, &m).unwrap();
        let client = Client {
            id: id.into(),
            jwks: m.jwks.clone(),
            metadata: m,
            is_trusted: false,
            is_first_party: false,
        };
        let now = 1_700_000_000;
        let assertion = |claims: Value| key.sign(Map::new(), &claims);
        let good = json!({"iss": id, "sub": id, "aud": "https://pds.example.com", "jti": "a1", "iat": now});
        let auth = client
            .authenticate(
                Some(CLIENT_ASSERTION_TYPE_JWT_BEARER),
                Some(&assertion(good.clone())),
                "https://pds.example.com",
                now,
            )
            .unwrap();
        assert!(
            matches!(&auth, ClientAuth::PrivateKeyJwt { jti, kid, .. } if jti == "a1" && kid == key.kid())
        );
        assert_eq!(
            serde_json::to_value(&auth).unwrap()["method"],
            "private_key_jwt"
        );
        let mut wrong_aud = good.clone();
        wrong_aud["aud"] = json!("https://other.example.com");
        assert!(
            client
                .authenticate(
                    Some(CLIENT_ASSERTION_TYPE_JWT_BEARER),
                    Some(&assertion(wrong_aud)),
                    "https://pds.example.com",
                    now
                )
                .is_err()
        );
        let other = Es256Key::generate();
        let forged = other.sign(Map::new(), &good);
        assert!(
            client
                .authenticate(
                    Some(CLIENT_ASSERTION_TYPE_JWT_BEARER),
                    Some(&forged),
                    "https://pds.example.com",
                    now
                )
                .is_err()
        );
        assert!(
            client
                .authenticate(
                    Some(CLIENT_ASSERTION_TYPE_JWT_BEARER),
                    Some(&assertion(good)),
                    "https://pds.example.com",
                    now + 120
                )
                .is_err(),
            "too old"
        );
        assert_eq!(
            client.refresh_lifetime_ms(),
            REFRESH_LIFETIME_EXTENDED_MS,
            "confidential"
        );
    }
}
