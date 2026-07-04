//! atproto-OAuth provider (Arc 2 Phase β, strangler-fig; chainlink #420).
//!
//! A new, atproto-spec-compliant OAuth authorization-server provider served
//! under the `/oauth/atproto/*` namespace, distinct from the legacy
//! `src/oauth/*` provider (retained for operator-internal use, untouched).
//! The strangler-fig boundary (SD-A2 = (c)) is made concrete in the URL space.
//!
//! Phase β.2 ships the resource-owner authentication substrate: the
//! [`browser_session`] store + the [`login`] endpoints (login-α). Phase β.3
//! adds the full provider surface on top: [`authorize`] → [`consent`] →
//! [`token`], plus [`par`] (pushed authorization requests), [`metadata`] (AS
//! discovery), and the parallel [`scope`] vocabulary. URL-based client trust
//! is resolved by the β.4 [`client_metadata`] fetcher.

pub mod authorize;
pub mod browser_session;
pub mod client_metadata;
pub mod consent;
pub mod device;
pub mod device_manager;
pub mod holder;
pub mod html;
pub mod login;
pub mod metadata;
pub mod par;
pub mod params;
pub mod request_store;
pub mod scope;
pub mod token;

use axum::http::{header, StatusCode};
use axum::response::Response;
use axum::routing::{get, post};
use axum::Router;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use rand::rngs::OsRng;
use rand::RngCore;

use crate::context::AppContext;
use crate::error::{PdsError, PdsResult};

/// Build the atproto-OAuth provider routes.
///
/// `/oauth/atproto/*` for the provider surface, plus the root-level
/// `/.well-known/oauth-authorization-server` AS-metadata discovery document.
/// Merged into the application router (strangler-fig boundary in URL space:
/// the legacy `/oauth/*` provider is untouched).
pub fn routes() -> Router<AppContext> {
    Router::new()
        // AS-login (login-α): GET issues a challenge, POST verifies + mints a
        // browser session.
        .route(
            "/oauth/atproto/login",
            get(login::challenge).post(login::verify),
        )
        .route("/oauth/atproto/logout", post(login::logout))
        // whoami — resolves the current session (exercises the validation
        // extractor; reused by β.3's authorize/consent handlers).
        .route("/oauth/atproto/session", get(login::whoami))
        // β.3 provider surface.
        .route("/oauth/atproto/authorize", get(authorize::authorize))
        .route(
            "/oauth/atproto/consent/approve",
            post(consent::approve),
        )
        .route("/oauth/atproto/consent/deny", post(consent::deny))
        .route("/oauth/atproto/token", post(token::token))
        .route("/oauth/atproto/par", post(par::par))
        .route(
            "/.well-known/oauth-authorization-server",
            get(metadata::authorization_server_metadata),
        )
        // ε.1 — holder device + grant management (browser-session authed).
        .route("/oauth/atproto/device/register", post(device::register))
        .route("/oauth/atproto/device/list", get(device::list))
        .route("/oauth/atproto/device/revoke", post(device::revoke))
        .route("/oauth/atproto/grant/list", get(device::grant_list))
        .route("/oauth/atproto/grant/revoke", post(device::grant_revoke))
        // Holder self-service UI (Phase 1, chainlink #424) — server-rendered
        // pages under `/oauth/atproto/holder/*`. The session cookie's
        // `Path=/oauth` scope reaches them, so the browser sends the holder's
        // session automatically. Sub-phase 1.1 merges the (empty) scaffold;
        // later sub-phases wire the login/home/devices/grants/preferences pages
        // into `holder::routes()`.
        .merge(holder::routes())
}

/// Generate a 256-bit opaque, URL-safe token — the CSPRNG primitive behind
/// request ids, authorization codes, and PAR request-uri opaque parts.
pub(super) fn opaque_token() -> String {
    let mut bytes = [0u8; 32];
    OsRng.fill_bytes(&mut bytes);
    URL_SAFE_NO_PAD.encode(bytes)
}

/// Verify a mandatory DPoP proof on a machine endpoint (token / PAR).
///
/// atproto OAuth requires DPoP on these endpoints, so an absent `DPoP` header
/// is itself an authentication failure (unlike the legacy token endpoint's
/// three-state bearer/DPoP semantics). On success returns the JWK thumbprint
/// — for PAR it confirms client-key possession; for the token endpoint it is
/// the binding committed onto (and, on refresh, matched against) the issued
/// token. `htu` is the full request URI the proof must commit to.
pub(super) async fn verify_dpop_required(
    ctx: &AppContext,
    headers: &axum::http::HeaderMap,
    htu: &str,
) -> PdsResult<String> {
    let raw = headers
        .get("DPoP")
        .ok_or_else(|| PdsError::Authentication("DPoP proof required".to_string()))?;
    let proof = raw
        .to_str()
        .map_err(|_| PdsError::Authentication("invalid DPoP header value".to_string()))?;
    ctx.dpop_verifier
        .verify_dpop_proof(proof, "POST", htu, None)
        .await
}

/// SHA-256 hex digest of a value (authorization codes, here). Reuses the same
/// primitive β.1 uses for bearer hashing — high-entropy random tokens, so the
/// digest is the correct at-rest representation (the raw code never lands on
/// disk).
pub(super) fn token_hash(value: &str) -> String {
    crate::oauth::access_token_hash(value)
}

/// Build an RFC 6749 §5.2 OAuth error response (`{"error", "error_description"}`)
/// for the machine endpoints (token / PAR). These speak the OAuth error shape
/// rather than Aurora's `PdsError` envelope so atproto clients can parse them.
pub(super) fn oauth_error_json(status: StatusCode, code: &str, description: &str) -> Response {
    let body = serde_json::json!({ "error": code, "error_description": description });
    // `serde_json::to_vec` on a plain object literal cannot fail; fall back to
    // a minimal body if it somehow does rather than panicking on the hot path.
    let bytes = serde_json::to_vec(&body).unwrap_or_else(|_| b"{\"error\":\"server_error\"}".to_vec());
    Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::CACHE_CONTROL, "no-store")
        .body(bytes.into())
        .expect("static header set builds a valid response")
}
