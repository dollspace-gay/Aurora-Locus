//! atproto-OAuth provider (Arc 2 Phase β, strangler-fig; chainlink #420).
//!
//! A new, atproto-spec-compliant OAuth authorization-server provider served
//! under the `/oauth/atproto/*` namespace, distinct from the legacy
//! `src/oauth/*` provider (retained for operator-internal use, untouched).
//! The strangler-fig boundary (SD-A2 = (c)) is made concrete in the URL space.
//!
//! Phase β.2 ships the resource-owner authentication substrate: the
//! [`browser_session`] store + the [`login`] endpoints (login-α). Later
//! sub-phases add the authorize / consent / token / PAR / metadata endpoints
//! (β.3) and URL-based client management (β.4) on top of this base.

pub mod browser_session;
pub mod login;

use axum::routing::{get, post};
use axum::Router;

use crate::context::AppContext;

/// Build the atproto-OAuth provider routes (`/oauth/atproto/*`).
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
}
