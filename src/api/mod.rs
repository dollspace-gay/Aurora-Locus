/// API routes and handlers
pub mod admin;
pub mod blob;
pub mod firehose;
pub mod identity;
pub mod labels;
pub mod middleware;
pub mod repo;
pub mod server;
pub mod sync;
pub mod well_known;

use crate::context::AppContext;
use axum::Router;

/// Build API routes
pub fn routes() -> Router<AppContext> {
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
}
