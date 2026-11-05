//! The well-known documents, as the reference builds them.

use serde_json::{Value, json};

use super::jose::VERIFY_ALGS;

/// `/.well-known/oauth-authorization-server` for `issuer` (an origin, no
/// trailing slash). `protected_resources` lists every origin served.
#[must_use]
pub fn authorization_server(issuer: &str, protected_resources: &[String]) -> Value {
    let url = |p: &str| format!("{issuer}{p}");
    let mut request_algs: Vec<&str> = VERIFY_ALGS.to_vec();
    request_algs.push("none");
    json!({
        "issuer": issuer,
        "scopes_supported": ["atproto", "transition:email", "transition:generic", "transition:chat.bsky"],
        "subject_types_supported": ["public"],
        "response_types_supported": ["code"],
        "response_modes_supported": ["query", "fragment", "form_post"],
        "grant_types_supported": ["authorization_code", "refresh_token"],
        "code_challenge_methods_supported": ["S256"],
        "ui_locales_supported": ["en-US"],
        "display_values_supported": ["page", "popup", "touch"],
        "prompt_values_supported": ["none", "login", "consent", "select_account", "create"],
        "authorization_response_iss_parameter_supported": true,
        "request_object_signing_alg_values_supported": request_algs,
        "request_object_encryption_alg_values_supported": [],
        "request_object_encryption_enc_values_supported": [],
        "request_parameter_supported": true,
        "request_uri_parameter_supported": true,
        "require_request_uri_registration": true,
        "jwks_uri": url("/oauth/jwks"),
        "authorization_endpoint": url("/oauth/authorize"),
        "token_endpoint": url("/oauth/token"),
        "token_endpoint_auth_methods_supported": ["none", "private_key_jwt"],
        "token_endpoint_auth_signing_alg_values_supported": VERIFY_ALGS,
        "revocation_endpoint": url("/oauth/revoke"),
        "pushed_authorization_request_endpoint": url("/oauth/par"),
        "require_pushed_authorization_requests": true,
        "dpop_signing_alg_values_supported": VERIFY_ALGS,
        "protected_resources": protected_resources,
        "client_id_metadata_document_supported": true,
    })
}

/// `/.well-known/oauth-protected-resource` for the resource at `resource`.
#[must_use]
pub fn protected_resource(resource: &str, authorization_server: &str) -> Value {
    json!({
        "resource": resource,
        "authorization_servers": [authorization_server],
        "scopes_supported": [],
        "bearer_methods_supported": ["header"],
        "resource_documentation": "https://atproto.com",
    })
}
