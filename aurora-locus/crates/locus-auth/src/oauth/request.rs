//! Authorization requests, as the reference's request parameters schema and
//! `RequestManager.validate`: parsing (JSON or form bodies), the atproto
//! rules (PKCE S256, `code` only, the `atproto` scope, consent for public
//! clients), scope narrowing to what the user approved, PKCE checks at the
//! token endpoint, and the opaque identifiers the flows hand out.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};

use super::OAuthError;
use super::client::{Client, redirect_uri_matches};

pub const REQUEST_URI_PREFIX: &str = "urn:ietf:params:oauth:request_uri:";
/// How long a pushed request lives before the user opens it.
pub const PAR_EXPIRES_MS: i64 = 5 * 60_000;
/// How long a request stays open between steps of the sign-in.
pub const AUTHORIZATION_INACTIVITY_MS: i64 = 5 * 60_000;
/// Access token lifetime.
pub const TOKEN_MAX_AGE_MS: i64 = 60 * 60_000;
/// How long a used code challenge is remembered.
pub const CODE_CHALLENGE_REPLAY_MS: i64 = 24 * 60 * 60_000 * 11 / 10;

fn hex_id(prefix: &str, bytes: usize) -> String {
    let raw: Vec<u8> = (0..bytes).map(|_| rand::random::<u8>()).collect();
    format!("{prefix}{}", hex::encode(raw))
}

#[must_use]
pub fn new_request_id() -> String {
    hex_id("req-", 16)
}
#[must_use]
pub fn new_code() -> String {
    hex_id("cod-", 32)
}
#[must_use]
pub fn new_token_id() -> String {
    hex_id("tok-", 16)
}
#[must_use]
pub fn new_refresh_token() -> String {
    hex_id("ref-", 32)
}
#[must_use]
pub fn new_device_id() -> String {
    hex_id("dev-", 16)
}
#[must_use]
pub fn new_session_id() -> String {
    hex_id("ses-", 16)
}

/// The id behind a `request_uri`.
#[must_use]
pub fn request_id_from_uri(uri: &str) -> Option<&str> {
    uri.strip_prefix(REQUEST_URI_PREFIX)
        .filter(|id| id.starts_with("req-") && id.len() == 36)
}

/// Authorization request parameters (stored as JSON with the request and
/// the grant).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Parameters {
    pub client_id: String,
    pub response_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub redirect_uri: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scope: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub state: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub code_challenge: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub code_challenge_method: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dpop_jkt: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub response_mode: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub nonce: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_age: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub claims: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub login_hint: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ui_locales: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id_token_hint: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub display: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prompt: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub authorization_details: Option<Value>,
}

/// The OAuth scope syntax (RFC 6749 §3.3).
#[must_use]
pub fn is_oauth_scope(s: &str) -> bool {
    !s.is_empty()
        && s.split(' ').all(|t| {
            !t.is_empty()
                && t.bytes()
                    .all(|b| b == 0x21 || (0x23..=0x5b).contains(&b) || (0x5d..=0x7e).contains(&b))
        })
}

fn str_field(m: &Map<String, Value>, k: &str) -> Result<Option<String>, String> {
    match m.get(k) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(s)) => Ok(Some(s.clone())),
        Some(_) => Err(format!("Invalid \"{k}\"")),
    }
}

fn enum_field(m: &Map<String, Value>, k: &str, allowed: &[&str]) -> Result<Option<String>, String> {
    let v = str_field(m, k)?;
    if let Some(s) = &v
        && !allowed.contains(&s.as_str())
    {
        return Err(format!("Invalid \"{k}\" value \"{s}\""));
    }
    Ok(v)
}

/// A JSON-object parameter (a JSON string in a form body).
fn json_field(m: &Map<String, Value>, k: &str) -> Result<Option<Value>, String> {
    match m.get(k) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(s)) => serde_json::from_str(s)
            .map(Some)
            .map_err(|_| format!("Invalid \"{k}\"")),
        Some(v) => Ok(Some(v.clone())),
    }
}

impl Parameters {
    /// Parses parameters from a JSON object or form fields (strings).
    pub fn parse(m: &Map<String, Value>) -> Result<Self, String> {
        let client_id = str_field(m, "client_id")?.ok_or("Missing \"client_id\"")?;
        let response_type = enum_field(
            m,
            "response_type",
            &[
                "code",
                "token",
                "none",
                "code id_token token",
                "code id_token",
                "code token",
                "id_token token",
                "id_token",
            ],
        )?
        .ok_or("Missing \"response_type\"")?;
        let scope = str_field(m, "scope")?;
        if scope.as_deref().is_some_and(|s| !is_oauth_scope(s)) {
            return Err("Invalid OAuth scope".into());
        }
        let max_age = match m.get("max_age") {
            None | Some(Value::Null) => None,
            Some(Value::Number(n)) => n.as_i64(),
            Some(Value::String(s)) => s.parse::<i64>().ok(),
            Some(_) => None,
        };
        if m.contains_key("max_age") && max_age.is_none_or(|a| a < 0) {
            return Err("Invalid \"max_age\"".into());
        }
        let login_hint = str_field(m, "login_hint")?;
        if login_hint.as_deref() == Some("") {
            return Err("Invalid \"login_hint\"".into());
        }
        Ok(Self {
            client_id,
            response_type,
            redirect_uri: str_field(m, "redirect_uri")?,
            scope,
            state: str_field(m, "state")?,
            code_challenge: str_field(m, "code_challenge")?,
            code_challenge_method: enum_field(m, "code_challenge_method", &["S256", "plain"])?,
            dpop_jkt: str_field(m, "dpop_jkt")?,
            response_mode: enum_field(m, "response_mode", &["query", "fragment", "form_post"])?,
            nonce: str_field(m, "nonce")?,
            max_age,
            claims: json_field(m, "claims")?,
            login_hint,
            ui_locales: str_field(m, "ui_locales")?,
            id_token_hint: str_field(m, "id_token_hint")?,
            display: enum_field(m, "display", &["page", "popup", "touch", "wap"])?,
            prompt: enum_field(
                m,
                "prompt",
                &["none", "login", "consent", "select_account", "create"],
            )?,
            authorization_details: json_field(m, "authorization_details")?,
        })
    }

    #[must_use]
    pub fn scopes(&self) -> Vec<&str> {
        self.scope
            .as_deref()
            .map(|s| s.split(' ').collect())
            .unwrap_or_default()
    }
}

/// An error tied to a request: the parameters it carries let the
/// authorize endpoint redirect it back to the client.
fn req_err(error: &str, m: impl Into<String>) -> OAuthError {
    OAuthError::new(error, m, 400)
}

/// The reference's `Client.validateRequest` then `RequestManager.validate`
/// (without the permission-set resolution, which the caller does).
pub fn validate(client: &Client, mut p: Parameters) -> Result<Parameters, OAuthError> {
    for (k, present) in [
        ("claims", p.claims.is_some()),
        ("id_token_hint", p.id_token_hint.is_some()),
        ("nonce", p.nonce.is_some()),
    ] {
        if present {
            return Err(req_err(
                "invalid_request",
                format!("Unsupported \"{k}\" parameter"),
            ));
        }
    }
    if p.response_type != "code" {
        return Err(req_err(
            "unsupported_response_type",
            format!("Unsupported response_type \"{}\"", p.response_type),
        ));
    }
    if let Some(d) = &p.authorization_details {
        let t = d
            .get(0)
            .and_then(|x| x.get("type"))
            .and_then(Value::as_str)
            .unwrap_or("");
        return Err(req_err(
            "invalid_authorization_details",
            format!("Unsupported \"authorization_details\" type \"{t}\""),
        ));
    }

    // Client.validateRequest.
    if p.client_id != client.id {
        return Err(req_err(
            "invalid_request",
            "The \"client_id\" parameter field does not match the value used to authenticate the client",
        ));
    }
    if let Some(scope) = &p.scope {
        let declared: Vec<&str> = client
            .metadata
            .scope
            .as_deref()
            .ok_or_else(|| {
                req_err(
                    "invalid_scope",
                    "Client has no declared scopes in its metadata",
                )
            })?
            .split(' ')
            .collect();
        for s in scope.split(' ') {
            if !declared.contains(&s) {
                return Err(req_err(
                    "invalid_scope",
                    format!("Scope \"{s}\" is not declared in the client metadata"),
                ));
            }
        }
    }
    if !client.metadata.response_types.contains(&p.response_type) {
        return Err(req_err(
            "invalid_request",
            format!(
                "Invalid response_type \"{}\" requested by the client",
                p.response_type
            ),
        ));
    }
    if !client
        .metadata
        .grant_types
        .iter()
        .any(|g| g == "authorization_code")
    {
        return Err(req_err(
            "invalid_request",
            "This client is not allowed to use the \"authorization_code\" grant type",
        ));
    }
    match &p.redirect_uri {
        Some(r) => {
            if !client
                .metadata
                .redirect_uris
                .iter()
                .any(|u| redirect_uri_matches(u, r))
            {
                return Err(req_err(
                    "invalid_request",
                    format!("Invalid redirect_uri {r}"),
                ));
            }
        }
        None => match client.default_redirect_uri() {
            Some(d) => p.redirect_uri = Some(d.to_owned()),
            None => return Err(req_err("invalid_request", "redirect_uri is required")),
        },
    }

    // Only scopes locus can interpret are kept.
    let had_atproto = p.scopes().contains(&"atproto");
    let had_openid = p.scopes().contains(&"openid");
    let mut seen: Vec<String> = Vec::new();
    for s in p.scopes() {
        if crate::scopes::is_atproto_scope(s) && !seen.iter().any(|x| x == s) {
            seen.push(s.to_owned());
        }
    }
    p.scope = (!seen.is_empty()).then(|| seen.join(" "));

    match (&p.code_challenge, &p.code_challenge_method) {
        (Some(_), None) => p.code_challenge_method = Some("plain".into()),
        (Some(_), Some(_)) => {}
        (None, Some(_)) => {
            return Err(req_err(
                "invalid_request",
                "code_challenge is required when code_challenge_method is provided",
            ));
        }
        (None, None) => return Err(req_err("invalid_request", "Use of PKCE is required")),
    }
    if !had_atproto {
        return Err(req_err(
            "invalid_scope",
            "The \"atproto\" scope is required",
        ));
    }
    if had_openid {
        return Err(req_err(
            "invalid_scope",
            "OpenID Connect is not compatible with atproto",
        ));
    }
    if p.code_challenge_method.as_deref() != Some("S256") {
        return Err(req_err(
            "invalid_request",
            "atproto requires use of \"S256\" code_challenge_method",
        ));
    }
    if !client.is_confidential() && !client.is_trusted {
        if p.prompt.as_deref() == Some("none") {
            return Err(req_err(
                "consent_required",
                "Public clients are not allowed to use silent-sign-on",
            ));
        }
        if p.prompt.as_deref() != Some("create") {
            p.prompt = Some("consent".into());
        }
    }
    if let Some(h) = p.login_hint.as_ref().map(|h| h.to_lowercase()) {
        let ok = (h.starts_with("did:plc:") || h.starts_with("did:web:"))
            && proto_blue_syntax::Did::new(&h).is_ok()
            || proto_blue_syntax::Handle::new(&h).is_ok();
        if !ok {
            return Err(req_err(
                "invalid_request",
                format!("Invalid login_hint \"{h}\""),
            ));
        }
        p.login_hint = Some(h);
    }
    Ok(p)
}

/// The scopes the user approved, as the reference's
/// `narrowAuthorizedParameters` (the grant records only these).
pub fn narrow(mut p: Parameters, approved: Option<&str>) -> Result<Parameters, OAuthError> {
    let Some(approved) = approved else {
        return Ok(p);
    };
    let allowed: Vec<&str> = approved.split(' ').collect();
    let kept: Vec<String> = p
        .scopes()
        .into_iter()
        .filter(|s| allowed.contains(s))
        .map(str::to_owned)
        .collect();
    if !kept.iter().any(|s| s == "atproto") {
        return Err(OAuthError::access_denied(
            "The \"atproto\" scope is required",
        ));
    }
    p.scope = Some(kept.join(" "));
    Ok(p)
}

/// PKCE at the token endpoint, as `validateCodeGrant` (the caller then
/// checks the challenge hasn't been used before).
pub fn check_code_grant(
    p: &Parameters,
    redirect_uri: &str,
    verifier: Option<&str>,
) -> Result<(), OAuthError> {
    if p.redirect_uri.as_deref() != Some(redirect_uri) {
        return Err(OAuthError::invalid_grant(
            "The redirect_uri parameter must match the one used in the authorization request",
        ));
    }
    match &p.code_challenge {
        Some(challenge) => {
            let v =
                verifier.ok_or_else(|| OAuthError::invalid_grant("code_verifier is required"))?;
            if v.len() < 43 {
                return Err(OAuthError::invalid_grant("code_verifier too short"));
            }
            let ok = match p.code_challenge_method.as_deref() {
                None | Some("plain") => challenge == v,
                Some("S256") => {
                    // Decoded as base64 (either alphabet), compared to the hash.
                    use base64::Engine;
                    let norm: String = challenge.replace('-', "+").replace('_', "/");
                    let decoded = base64::engine::general_purpose::STANDARD_NO_PAD
                        .decode(norm.trim_end_matches('='))
                        .unwrap_or_default();
                    decoded == Sha256::digest(v.as_bytes()).as_slice()
                }
                Some(_) => false,
            };
            if !ok {
                return Err(OAuthError::invalid_grant("Invalid code_verifier"));
            }
            Ok(())
        }
        None if verifier.is_some() => Err(OAuthError::invalid_request(
            "code_challenge parameter wasn't provided",
        )),
        None => Ok(()),
    }
}

/// The code verifier syntax (RFC 7636).
#[must_use]
pub fn is_code_verifier(v: &str) -> bool {
    (43..=128).contains(&v.len())
        && v.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-._~".contains(&b))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::oauth::client::{ClientMetadata, loopback_metadata};
    use crate::oauth::jose::b64;
    use serde_json::json;

    fn public_client() -> Client {
        let m = ClientMetadata::parse(&json!({
            "client_id": "https://app.example.com/meta.json",
            "scope": "atproto transition:generic repo:app.bsky.feed.post",
            "redirect_uris": ["https://app.example.com/cb"],
            "grant_types": ["authorization_code", "refresh_token"],
            "token_endpoint_auth_method": "none",
            "dpop_bound_access_tokens": true,
        }))
        .unwrap();
        Client {
            id: "https://app.example.com/meta.json".into(),
            jwks: None,
            metadata: m,
            is_trusted: false,
            is_first_party: false,
        }
    }

    fn params(extra: Value) -> Parameters {
        let mut m = json!({
            "client_id": "https://app.example.com/meta.json",
            "response_type": "code",
            "scope": "atproto transition:generic",
            "code_challenge": b64(&Sha256::digest(b"verifier-verifier-verifier-verifier-verifier")),
            "code_challenge_method": "S256",
        });
        for (k, v) in extra.as_object().unwrap() {
            m[k] = v.clone();
        }
        Parameters::parse(m.as_object().unwrap()).unwrap()
    }

    #[test]
    fn requests_are_validated_as_the_reference() {
        let c = public_client();
        let p = validate(&c, params(json!({}))).unwrap();
        assert_eq!(
            p.redirect_uri.as_deref(),
            Some("https://app.example.com/cb"),
            "the only redirect uri"
        );
        assert_eq!(
            p.prompt.as_deref(),
            Some("consent"),
            "public clients always ask"
        );
        let err = |extra: Value| validate(&c, params(extra)).unwrap_err();
        assert_eq!(
            err(json!({"scope": "transition:generic"})).description,
            "The \"atproto\" scope is required"
        );
        assert!(
            err(json!({"scope": "atproto rpc:x.y.z?aud=*"}))
                .description
                .contains("not declared")
        );
        assert_eq!(
            err(json!({"code_challenge_method": "plain"})).description,
            "atproto requires use of \"S256\" code_challenge_method"
        );
        assert_eq!(err(json!({"prompt": "none"})).error, "consent_required");
        assert_eq!(
            err(json!({"nonce": "n"})).description,
            "Unsupported \"nonce\" parameter"
        );
        assert!(
            err(json!({"redirect_uri": "https://evil.example.com/cb"}))
                .description
                .starts_with("Invalid redirect_uri")
        );
        assert_eq!(
            err(json!({"login_hint": "not a handle"})).description,
            "Invalid login_hint \"not a handle\""
        );
        let mut no_pkce = params(json!({}));
        no_pkce.code_challenge = None;
        no_pkce.code_challenge_method = None;
        assert_eq!(
            validate(&c, no_pkce).unwrap_err().description,
            "Use of PKCE is required"
        );
        // A loopback client takes any port on its loopback redirect.
        let lb = loopback_metadata("http://localhost").unwrap();
        let lc = Client {
            id: "http://localhost".into(),
            jwks: None,
            metadata: ClientMetadata::parse(&lb).unwrap(),
            is_trusted: false,
            is_first_party: false,
        };
        let mut p = params(
            json!({"client_id": "http://localhost", "redirect_uri": "http://127.0.0.1:5173/"}),
        );
        p.scope = Some("atproto".into());
        validate(&lc, p).unwrap();
    }

    #[test]
    fn narrowing_and_pkce() {
        let p = params(
            json!({"scope": "atproto transition:generic transition:email", "redirect_uri": "https://app.example.com/cb"}),
        );
        assert_eq!(
            narrow(p.clone(), Some("atproto transition:email"))
                .unwrap()
                .scope
                .as_deref(),
            Some("atproto transition:email")
        );
        assert_eq!(
            narrow(p.clone(), Some("transition:email"))
                .unwrap_err()
                .error,
            "access_denied"
        );
        let v = "verifier-verifier-verifier-verifier-verifier";
        check_code_grant(&p, "https://app.example.com/cb", Some(v)).unwrap();
        assert_eq!(
            check_code_grant(&p, "https://app.example.com/cb", Some(&format!("{v}x")))
                .unwrap_err()
                .description,
            "Invalid code_verifier"
        );
        assert_eq!(
            check_code_grant(&p, "https://other.example.com/cb", Some(v))
                .unwrap_err()
                .error,
            "invalid_grant"
        );
        assert_eq!(
            check_code_grant(&p, "https://app.example.com/cb", None)
                .unwrap_err()
                .description,
            "code_verifier is required"
        );
        assert!(is_code_verifier(v) && !is_code_verifier("short"));
        let id = new_request_id();
        assert_eq!(
            request_id_from_uri(&format!("{REQUEST_URI_PREFIX}{id}")),
            Some(id.as_str())
        );
        assert!(request_id_from_uri("urn:ietf:params:oauth:request_uri:nope").is_none());
    }
}
