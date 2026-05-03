/// API routes and handlers
pub mod admin;
pub mod appview;
pub mod aurora_admin;
pub mod aurora_moderator;
pub mod blob;
pub mod federation;
pub mod firehose;
pub mod health;
pub mod identity;
pub mod labels;
pub mod middleware;
pub mod moderation;
pub mod oauth_admin;
pub mod oauth_server;
pub mod repo;
pub mod server;
pub mod sync;
pub mod sync_helpers;
pub mod temp;
pub mod well_known;

use crate::context::AppContext;
use axum::Router;

/// Build API routes
pub fn routes() -> Router<AppContext> {
    // Create OAuth state store (in-memory for now)
    let oauth_state_store = oauth_admin::OAuthStateStore::new();

    Router::new()
        .merge(well_known::routes())
        .merge(server::routes())
        .merge(repo::routes())
        .merge(blob::routes())
        .merge(identity::routes())
        .merge(admin::routes())
        .merge(sync::routes())
        .merge(firehose::routes())
        .merge(labels::routes())
        .merge(moderation::routes())
        .merge(health::routes())
        .merge(federation::routes())
        .merge(temp::routes())
        .merge(appview::routes()) // AppView proxy with read-after-write
        // OAuth admin routes with their own state
        .merge(oauth_admin::routes(oauth_state_store))
        // OAuth server routes (for third-party app authorization)
        .merge(oauth_server::routes())
}
