//! did:web holder self-service UI (Phase 1, chainlink #424).
//!
//! Server-rendered HTML pages that let a did:web account holder manage their
//! own presence on this PDS: sign in (login-α challenge, secp256k1), review
//! their devices and OAuth grants, set a display-theme preference, and sign
//! out. The pages live under `/oauth/atproto/holder/*` so the
//! `aurora_oauth_session` cookie (`Path=/oauth`, see
//! [`super::browser_session`]) reaches them without widening the cookie scope.
//!
//! Rendering follows the provider convention: hand-rolled `format!` →
//! [`axum::response::Html`], every holder-supplied value escaped via
//! [`super::html::html_escape`]. Authentication reuses the β.2
//! [`super::browser_session::BrowserSessionContext`] extractor; every mutating
//! POST carries the per-session CSRF token, matching the consent screen.
//!
//! Client-side cryptography (secp256k1 challenge signing for login, P-256 DPoP
//! keypair generation for device registration) runs in small JS islands served
//! as static assets under `/holder/*` (mounted in [`crate::server`]).
//!
//! ## Sub-phase build-out
//!
//! Phase 1 lands incrementally; [`routes`] grows one sub-phase at a time:
//!
//! - **1.1 (this commit):** module scaffold — [`routes`] returns an empty
//!   router. The `/holder/*` static mount, the shared `html_escape`
//!   extraction, and the `atproto_holder_preferences` table land alongside.
//! - **1.2:** login page (GET/POST) + secp256k1 signing island.
//! - **1.3:** home, devices, grants pages (+ P-256 DPoP-keygen island).
//! - **1.4:** preferences page + logout, with the per-holder theme picker.

use axum::routing::get;
use axum::Router;

use crate::context::AppContext;

pub mod auth_method_manager;
pub mod login;
pub mod view;

/// Build the holder self-service routes, merged into the atproto-OAuth provider
/// router by [`super::routes`].
///
/// Phase 1 wires the login surface here; later commits add the interior pages
/// (home / devices / grants / preferences / auth-methods) + logout. All pages
/// sit under `/oauth/atproto/holder/*`, inside the session cookie's
/// `Path=/oauth` scope.
pub fn routes() -> Router<AppContext> {
    Router::new()
        // Holder login (password; passkey + login-α deferred). Pre-auth: the
        // GET renders the form, the POST verifies the credential and mints a
        // browser session. No session-CSRF token (there is no session yet — the
        // security property is the credential itself, matching β.2's AS-login).
        .route(
            "/oauth/atproto/holder/login",
            get(login::login_page).post(login::submit_login),
        )
}
