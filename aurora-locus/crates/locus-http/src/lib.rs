//! The HTTP layer: axum routing, XRPC plumbing, pipethrough, rate limiting
//! and every handler.

pub mod auth;
pub mod error;
pub mod identity;
pub mod server;
pub mod state;

#[cfg(test)]
mod tests;

use std::sync::Arc;

use axum::Router;
use axum::routing::{get, post};
use tokio_util::sync::CancellationToken;

pub use state::{AppState, ServerSecrets};

/// Every route.
pub fn router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/xrpc/_health", get(identity::health))
        .route(
            "/xrpc/com.atproto.server.describeServer",
            get(server::describe_server),
        )
        .route(
            "/xrpc/com.atproto.server.createAccount",
            post(server::create_account),
        )
        .route(
            "/xrpc/com.atproto.server.createSession",
            post(server::create_session),
        )
        .route(
            "/xrpc/com.atproto.server.getSession",
            get(server::get_session),
        )
        .route(
            "/xrpc/com.atproto.server.refreshSession",
            post(server::refresh_session),
        )
        .route(
            "/xrpc/com.atproto.server.deleteSession",
            post(server::delete_session),
        )
        .route(
            "/xrpc/com.atproto.server.createAppPassword",
            post(server::create_app_password),
        )
        .route(
            "/xrpc/com.atproto.server.listAppPasswords",
            get(server::list_app_passwords),
        )
        .route(
            "/xrpc/com.atproto.server.revokeAppPassword",
            post(server::revoke_app_password),
        )
        .route(
            "/xrpc/com.atproto.identity.resolveHandle",
            get(identity::resolve_handle),
        )
        .route(
            "/.well-known/atproto-did",
            get(identity::well_known_atproto_did),
        )
        .route("/.well-known/did.json", get(identity::well_known_did_json))
        .with_state(state)
}

/// Serves on `listener` until `cancel` fires, then drains in-flight requests.
pub async fn serve(
    state: Arc<AppState>,
    listener: tokio::net::TcpListener,
    cancel: CancellationToken,
) -> std::io::Result<()> {
    axum::serve(listener, router(state))
        .with_graceful_shutdown(async move { cancel.cancelled().await })
        .await
}
