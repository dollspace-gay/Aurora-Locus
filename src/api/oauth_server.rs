//! OAuth 2.1 Authorization Server Routes
//!
//! This module provides OAuth 2.1 server endpoints for third-party app authorization:
//! - `/oauth/authorize` - Authorization endpoint (PKCE flow)
//! - `/oauth/consent` - Consent screen for user approval
//! - `/oauth/token` - Token endpoint (exchange code for tokens, refresh tokens)
//! - `/oauth/clients` - List/manage authorized clients
//! - `/oauth/devices` - List/manage authorized devices
//!
//! These endpoints enable third-party applications to connect to user accounts
//! via OAuth 2.1 with PKCE and optional DPoP token binding.

use crate::{
    context::AppContext,
    oauth::{authorize, consent_screen, deny_authorization, grant_authorization, token_endpoint},
};
use axum::{
    routing::{get, post},
    Router,
};

/// Build OAuth server routes
///
/// These routes implement the OAuth 2.1 authorization server spec,
/// allowing third-party apps to obtain access tokens for user accounts.
pub fn routes() -> Router<AppContext> {
    Router::new()
        // OAuth 2.1 core endpoints
        .route("/oauth/authorize", get(authorize))
        .route("/oauth/consent", get(consent_screen))
        .route("/oauth/consent/approve", post(grant_authorization))
        .route("/oauth/consent/deny", post(deny_authorization))
        .route("/oauth/token", post(token_endpoint))
    // TODO: Add user-facing endpoints for managing authorized apps and devices:
    // .route("/oauth/authorized-clients", get(list_authorized_clients))
    // .route("/oauth/authorized-clients/:client_id", delete(revoke_client))
    // .route("/oauth/devices", get(list_devices))
    // .route("/oauth/devices/:device_id", delete(revoke_device))
}
