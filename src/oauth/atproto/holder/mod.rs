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

use axum::routing::{get, post};
use axum::Router;

use crate::context::AppContext;
use crate::oauth::atproto::browser_session::BrowserSessionContext;

pub mod auth_method_manager;
pub mod auth_methods;
pub mod devices;
pub mod grants;
pub mod home;
pub mod login;
pub mod logout;
pub mod preferences;
pub mod preferences_manager;
pub mod view;

/// Where unauthenticated holder pages send the browser.
pub(crate) const LOGIN_PATH: &str = "/oauth/atproto/holder/login";
/// The holder home / landing page.
pub(crate) const HOME_PATH: &str = "/oauth/atproto/holder/home";

/// CSRF check for holder form POSTs: the submitted token must equal the
/// session's per-session token (β.3 consent discipline; the `request_id`/page
/// is never a trust token).
pub(crate) fn csrf_ok(session: &BrowserSessionContext, presented: &str) -> bool {
    presented == session.session.csrf_token
}

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
        // login-α challenge issuance (consumed by login-alpha.js).
        .route(
            "/oauth/atproto/holder/login/challenge",
            get(login::login_challenge),
        )
        // Landing page + sign-out.
        .route("/oauth/atproto/holder/home", get(home::home_page))
        .route("/oauth/atproto/holder/logout", post(logout::logout))
        // Sign-in method management (password add/remove/set-primary; passkey +
        // login-α opt-in gated by `holder_login_alpha_enabled`).
        .route(
            "/oauth/atproto/holder/auth-methods",
            get(auth_methods::auth_methods_page),
        )
        .route(
            "/oauth/atproto/holder/auth-methods/add-password",
            post(auth_methods::add_password),
        )
        .route(
            "/oauth/atproto/holder/auth-methods/add-login-alpha",
            post(auth_methods::add_login_alpha),
        )
        .route(
            "/oauth/atproto/holder/auth-methods/remove",
            post(auth_methods::remove),
        )
        .route(
            "/oauth/atproto/holder/auth-methods/set-primary",
            post(auth_methods::set_primary),
        )
        // Per-holder display preferences (theme picker).
        .route(
            "/oauth/atproto/holder/preferences",
            get(preferences::preferences_page).post(preferences::set_preferences),
        )
        // Device management (list + revoke; registration happens in the OAuth
        // client flow, not here).
        .route("/oauth/atproto/holder/devices", get(devices::devices_page))
        .route(
            "/oauth/atproto/holder/devices/revoke",
            post(devices::revoke),
        )
        // Connected apps (OAuth grants) — list + revoke.
        .route("/oauth/atproto/holder/grants", get(grants::grants_page))
        .route("/oauth/atproto/holder/grants/revoke", post(grants::revoke))
}
