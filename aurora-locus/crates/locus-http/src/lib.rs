//! The HTTP layer: axum routing, XRPC plumbing, pipethrough, rate limiting
//! and every handler.

pub mod account;
pub mod auth;
pub mod email;
pub mod error;
pub mod identity;
pub mod repo;
pub mod routing;
pub mod server;
pub mod state;

#[cfg(test)]
mod tests;

use std::sync::Arc;

use axum::Router;
use axum::routing::{get, post};
use tokio_util::sync::CancellationToken;

pub use state::{AppState, ResolverCache, ServerSecrets};

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
            "/xrpc/com.atproto.server.requestEmailConfirmation",
            post(email::request_email_confirmation),
        )
        .route(
            "/xrpc/com.atproto.server.confirmEmail",
            post(email::confirm_email),
        )
        .route(
            "/xrpc/com.atproto.server.requestEmailUpdate",
            post(email::request_email_update),
        )
        .route(
            "/xrpc/com.atproto.server.updateEmail",
            post(email::update_email),
        )
        .route(
            "/xrpc/com.atproto.server.requestPasswordReset",
            post(email::request_password_reset),
        )
        .route(
            "/xrpc/com.atproto.server.resetPassword",
            post(email::reset_password),
        )
        .route(
            "/xrpc/com.atproto.server.getAccountInviteCodes",
            get(server::get_account_invite_codes),
        )
        .route(
            "/xrpc/com.atproto.temp.checkSignupQueue",
            get(server::check_signup_queue),
        )
        .route(
            "/xrpc/com.atproto.server.getServiceAuth",
            get(server::get_service_auth),
        )
        .route(
            "/xrpc/com.atproto.server.deactivateAccount",
            post(account::deactivate_account),
        )
        .route(
            "/xrpc/com.atproto.server.activateAccount",
            post(account::activate_account),
        )
        .route(
            "/xrpc/com.atproto.server.checkAccountStatus",
            get(account::check_account_status),
        )
        .route(
            "/xrpc/com.atproto.server.requestAccountDelete",
            post(account::request_account_delete),
        )
        .route(
            "/xrpc/com.atproto.server.deleteAccount",
            post(account::delete_account),
        )
        .route(
            "/xrpc/com.atproto.repo.createRecord",
            post(repo::create_record),
        )
        .route("/xrpc/com.atproto.repo.putRecord", post(repo::put_record))
        .route(
            "/xrpc/com.atproto.repo.deleteRecord",
            post(repo::delete_record),
        )
        .route(
            "/xrpc/com.atproto.repo.applyWrites",
            post(repo::apply_writes),
        )
        .route("/xrpc/com.atproto.repo.getRecord", get(repo::get_record))
        .route(
            "/xrpc/com.atproto.repo.listRecords",
            get(repo::list_records),
        )
        .route(
            "/xrpc/com.atproto.repo.describeRepo",
            get(repo::describe_repo),
        )
        .route(
            "/xrpc/com.atproto.repo.uploadBlob",
            post(repo::upload_blob).layer(axum::extract::DefaultBodyLimit::disable()),
        )
        .route("/xrpc/com.atproto.sync.getBlob", get(repo::get_blob))
        .route("/blob/{did}/{cid}", get(repo::get_blob_path))
        .route(
            "/xrpc/com.atproto.identity.resolveHandle",
            get(identity::resolve_handle),
        )
        .route(
            "/xrpc/com.atproto.identity.updateHandle",
            post(identity::update_handle),
        )
        .route(
            "/xrpc/com.atproto.identity.getRecommendedDidCredentials",
            get(identity::get_recommended_did_credentials),
        )
        .route(
            "/xrpc/com.atproto.identity.requestPlcOperationSignature",
            post(identity::request_plc_operation_signature),
        )
        .route(
            "/xrpc/com.atproto.identity.signPlcOperation",
            post(identity::sign_plc_operation),
        )
        .route(
            "/xrpc/com.atproto.identity.submitPlcOperation",
            post(identity::submit_plc_operation),
        )
        .route(
            "/.well-known/atproto-did",
            get(identity::well_known_atproto_did),
        )
        .route("/.well-known/did.json", get(identity::well_known_did_json))
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            routing::by_host,
        ))
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
