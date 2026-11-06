//! The HTTP layer: axum routing, XRPC plumbing, pipethrough, rate limiting
//! and every handler.

pub mod account;
pub mod acme;
pub mod admin;
pub mod admin_auth;
pub mod audience;
pub mod audience_sync;
pub mod auth;
pub mod backups;
pub mod blob_migration;
pub mod branding;
pub mod bsky;
pub mod cloudflare;
pub mod console;
pub mod console_people;
pub mod console_sections;
pub mod email;
pub mod entryway;
pub mod error;
pub mod health;
pub mod hostnames;
pub mod identity;
pub mod import;
pub mod keys;
pub mod kryphocron;
pub mod lexicon;
pub mod locus_admin;
pub mod media_gate;
pub mod membership;
pub mod metrics;
pub mod oauth;
pub mod oauth_ui;
pub mod pipethrough;
pub mod portal;
pub mod ratelimit;
pub mod raw;
pub mod repair;
pub mod repo;
pub mod routing;
pub mod scanner;
pub mod scratch;
pub mod server;
pub mod setup;
pub mod shard;
pub mod socket;
pub mod state;
pub mod sync;
pub mod web;

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
        .route("/xrpc/_health", get(health::health))
        .route("/healthz", get(health::healthz))
        .route("/metrics", get(metrics::serve))
        .route("/readyz", get(health::readyz))
        .route("/tls-check", get(health::tls_check))
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
            "/xrpc/com.atproto.server.reserveSigningKey",
            post(shard::reserve_signing_key),
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
        .route("/xrpc/com.atproto.repo.uploadBlob", post(repo::upload_blob))
        .route(
            "/xrpc/com.atproto.repo.importRepo",
            post(import::import_repo),
        )
        .route(
            "/xrpc/com.atproto.repo.listMissingBlobs",
            get(import::list_missing_blobs),
        )
        .route(
            concat!("/xrpc/", locus_core::locus_nsid!("account.getImportStatus")),
            get(import::get_import_status),
        )
        .route(
            "/xrpc/tools.kryphocron.feed.createPostPrivate",
            post(kryphocron::create_post_private),
        )
        .route(
            "/xrpc/tools.kryphocron.feed.updatePostPrivate",
            post(kryphocron::update_post_private),
        )
        .route(
            "/xrpc/tools.kryphocron.feed.deletePostPrivate",
            post(kryphocron::delete_post_private),
        )
        .route(
            "/xrpc/com.atproto.admin.getAccountInfo",
            get(admin::get_account_info),
        )
        .route(
            "/xrpc/com.atproto.admin.getAccountInfos",
            get(admin::get_account_infos),
        )
        .route(
            "/xrpc/com.atproto.admin.searchAccounts",
            get(admin::search_accounts),
        )
        .route(
            "/xrpc/com.atproto.admin.getSubjectStatus",
            get(admin::get_subject_status),
        )
        .route(
            "/xrpc/com.atproto.admin.updateSubjectStatus",
            post(admin::update_subject_status),
        )
        .route("/xrpc/com.atproto.admin.sendEmail", post(admin::send_email))
        .route(
            "/xrpc/com.atproto.admin.deleteAccount",
            post(admin::delete_account),
        )
        .route(
            "/xrpc/com.atproto.admin.updateAccountPassword",
            post(admin::update_account_password),
        )
        .route(
            "/xrpc/com.atproto.admin.updateAccountEmail",
            post(admin::update_account_email),
        )
        .route(
            "/xrpc/com.atproto.admin.updateAccountHandle",
            post(admin::update_account_handle),
        )
        .route(
            "/xrpc/com.atproto.admin.getInviteCodes",
            get(admin::get_invite_codes),
        )
        .route(
            "/xrpc/com.atproto.admin.disableAccountInvites",
            post(admin::disable_account_invites),
        )
        .route(
            "/xrpc/com.atproto.admin.enableAccountInvites",
            post(admin::enable_account_invites),
        )
        .route(
            "/xrpc/com.atproto.admin.disableInviteCodes",
            post(admin::disable_invite_codes),
        )
        .route(
            "/xrpc/com.atproto.server.createInviteCode",
            post(admin::create_invite_code),
        )
        .route(
            "/xrpc/com.atproto.server.createInviteCodes",
            post(admin::create_invite_codes),
        )
        .route(
            "/xrpc/tools.kryphocron.actor.participatePrivate",
            post(kryphocron::participate_private),
        )
        .route(
            concat!(
                "/xrpc/",
                locus_core::locus_nsid!("audience.getMembershipAssertion")
            ),
            get(membership::get_membership_assertion),
        )
        .route(
            "/xrpc/tools.kryphocron.policy.manageAudience",
            post(audience::manage_audience),
        )
        .route(
            "/xrpc/tools.kryphocron.policy.deleteAudience",
            post(audience::delete_audience),
        )
        .route("/xrpc/com.atproto.sync.getBlob", get(repo::get_blob))
        .route(
            "/xrpc/com.atproto.sync.subscribeRepos",
            get(sync::subscribe_repos),
        )
        .route("/xrpc/com.atproto.sync.getRepo", get(sync::get_repo))
        .route(
            "/xrpc/com.atproto.sync.getCheckout",
            get(sync::get_checkout),
        )
        .route("/xrpc/com.atproto.sync.getRecord", get(sync::get_record))
        .route("/xrpc/com.atproto.sync.getBlocks", get(sync::get_blocks))
        .route(
            "/xrpc/com.atproto.sync.getLatestCommit",
            get(sync::get_latest_commit),
        )
        .route("/xrpc/com.atproto.sync.getHead", get(sync::get_head))
        .route(
            "/xrpc/com.atproto.sync.getRepoStatus",
            get(sync::get_repo_status),
        )
        .route("/xrpc/com.atproto.sync.listRepos", get(sync::list_repos))
        .route("/xrpc/com.atproto.sync.listBlobs", get(sync::list_blobs))
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
        .route("/.well-known/change-password", get(shard::change_password))
        .route(
            "/.well-known/oauth-authorization-server",
            get(oauth::authorization_server_metadata),
        )
        .route(
            "/.well-known/oauth-protected-resource",
            get(oauth::protected_resource_metadata),
        )
        .route("/oauth/jwks", get(oauth::jwks))
        .route("/oauth/par", post(oauth::par).options(oauth::preflight))
        .route("/oauth/token", post(oauth::token).options(oauth::preflight))
        .route(
            "/oauth/revoke",
            post(oauth::revoke).options(oauth::preflight),
        )
        .route("/oauth/authorize", get(oauth_ui::authorize_page))
        .route("/oauth/authorize/sign-in", post(oauth_ui::sign_in))
        .route("/oauth/authorize/select", post(oauth_ui::select))
        .route("/oauth/authorize/accept", post(oauth_ui::accept))
        .route("/oauth/authorize/reject", post(oauth_ui::reject))
        .route("/oauth/authorize/sign-up", post(oauth_ui::sign_up))
        .route(
            "/oauth/authorize/show-sign-up",
            post(oauth_ui::show_sign_up),
        )
        .route(
            "/oauth/authorize/show-sign-in",
            post(oauth_ui::show_sign_in),
        )
        .route("/oauth/assets/style.css", get(oauth_ui::style))
        .route("/oauth/assets/form-post.js", get(oauth_ui::form_post_js))
        .route("/static/{*path}", get(web::asset))
        .route("/setup", get(setup::index))
        .route("/setup/token", post(setup::enter_token))
        .route("/setup/keys/backup", post(setup::key_backup))
        .route("/setup/probe", get(setup::probe_answer))
        .route(
            "/setup/{step}",
            get(setup::show_step).post(setup::save_step),
        )
        .route(
            "/xrpc/app.bsky.actor.getPreferences",
            get(bsky::get_preferences),
        )
        .route(
            "/xrpc/app.bsky.actor.putPreferences",
            post(bsky::put_preferences),
        )
        .route("/xrpc/app.bsky.actor.getProfile", get(raw::get_profile))
        .route("/xrpc/app.bsky.actor.getProfiles", get(raw::get_profiles))
        .route("/xrpc/app.bsky.feed.getTimeline", get(raw::get_timeline))
        .route(
            "/xrpc/app.bsky.feed.getAuthorFeed",
            get(raw::get_author_feed),
        )
        .route(
            "/xrpc/app.bsky.feed.getActorLikes",
            get(raw::get_actor_likes),
        )
        .route(
            "/xrpc/app.bsky.feed.getPostThread",
            get(raw::get_post_thread),
        )
        .route("/xrpc/app.bsky.feed.getFeed", get(raw::get_feed))
        .route(
            "/xrpc/app.bsky.notification.registerPush",
            post(raw::register_push),
        )
        .route(
            "/xrpc/app.bsky.notification.unregisterPush",
            post(raw::unregister_push),
        )
        // Locus's operator methods, and the console.
        .merge(locus_admin::routes())
        .merge(console::routes())
        .merge(console_people::routes())
        .merge(console_sections::routes())
        .merge(portal::routes())
        .merge(branding::routes())
        .merge(entryway::routes())
        .merge(repair::routes())
        // Every other XRPC method goes to another service.
        .route("/xrpc/{nsid}", axum::routing::any(pipethrough::proxy))
        // In the shard role, account management goes to the entryway; in
        // the entryway role, everything else goes to the account's shard.
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            shard::intercept,
        ))
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            entryway::intercept,
        ))
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            routing::limit_body,
        ))
        .layer(axum::extract::DefaultBodyLimit::disable())
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            ratelimit::middleware,
        ))
        .layer(axum::middleware::from_fn(routing::xrpc_rejections))
        .layer(axum::middleware::from_fn(metrics::record))
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            routing::headers_and_deadline,
        ))
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            setup::gate,
        ))
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
    axum::serve(
        listener,
        // A socket marker never comes in over TCP.
        router(state)
            .layer(axum::middleware::map_request(socket::strip))
            .into_make_service_with_connect_info::<std::net::SocketAddr>(),
    )
    .with_graceful_shutdown(async move { cancel.cancelled().await })
    .await
}
