/// API routes and handlers
pub mod admin;
pub mod appview;
pub mod aurora_admin;
pub mod aurora_lexicon;
pub mod extractors;
pub mod aurora_moderator;
pub mod aurora_subscribe;
pub mod blob;
#[cfg(debug_assertions)]
pub mod dev_routes;
pub mod federation;
pub mod account_emit;
pub mod firehose;
pub mod firehose_encoder;
pub mod health;
pub mod identity;
pub mod labels;
pub mod middleware;
pub mod moderation;
pub mod oauth_admin;
pub mod oauth_server;
pub mod kryphocron_endpoints;
pub mod registry;
pub mod repo;
pub mod repo_import;
pub mod server;
pub mod sync;
pub mod sync_helpers;
pub mod temp;
pub mod well_known;

use crate::context::AppContext;
use axum::Router;
use std::sync::Arc;

/// Build API routes.
///
/// Arc 8 Step 2 (chainlink #54): `admin::routes()` now returns a
/// `(Router, Arc<RouteRegistry>)` tuple — the populated registry
/// is propagated up so `main.rs` can hand it to
/// `AppContext::new`. The other sub-routers contribute no
/// registry entries (admin-tier routes all live in
/// `admin::routes()` per Step 0 Q1), so we forward the
/// registry verbatim.
pub fn routes() -> (Router<AppContext>, Arc<crate::api::registry::RouteRegistry>) {
    // Create OAuth state store (in-memory for now)
    let oauth_state_store = oauth_admin::OAuthStateStore::new();

    let (admin_router, registry) = admin::routes();

    let router = Router::new()
        .merge(well_known::routes())
        .merge(server::routes())
        .merge(repo::routes())
        .merge(repo_import::routes())
        .merge(kryphocron_endpoints::routes())
        .merge(blob::routes())
        .merge(identity::routes())
        .merge(admin_router)
        // v0.9 Arc B — resolved active-theme token CSS (§11). Unauthenticated
        // (loaded by the admin UI via a <link> tag); State<AppContext> for the
        // theme registry.
        .route(
            "/theme/active.css",
            axum::routing::get(crate::api::aurora_admin::serve_active_theme_css),
        )
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
        .merge(oauth_server::routes());

    // Arc 11 (chainlink #56): localhost-only dev curl framework
    // under `dev.aurora.*`. Compiled into debug builds only —
    // release builds never include the merge. List C by design;
    // not registered in `RouteRegistry` and not advertised by
    // `tools.aurora.describeCapabilities`. See
    // `docs/internal/dev-routes.md`.
    #[cfg(debug_assertions)]
    let router = router.merge(dev_routes::routes());

    (router, registry)
}
