/// API routes and handlers
pub mod admin;
pub mod appview;
pub mod aurora_admin;
pub mod aurora_kryphocron_ops;
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
pub mod auto_label_rules;
pub mod escalation_rules;
pub mod federation_discovery;
pub mod federation_peers;
pub mod integration_hooks;
pub mod lexicon_migration;
pub mod moderation;
pub mod moderation_defaults;
pub mod reviewer_assignment;
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
        // v0.9 Arc B §215 — resolved active-theme effect-class CSS (§11.6).
        // Sibling of active.css; same unauthenticated <link>-loaded contract.
        .route(
            "/theme/active-effects.css",
            axum::routing::get(crate::api::aurora_admin::serve_active_theme_effects_css),
        )
        // v0.9 Arc C §11.7 / #285 — resolved active-theme extension-point CSS
        // + the effective extension-point list (JSON) the runtime caches.
        // Same unauthenticated <link>/fetch contract as the siblings above.
        .route(
            "/theme/active-extensions.css",
            axum::routing::get(crate::api::aurora_admin::serve_active_theme_extensions_css),
        )
        .route(
            "/theme/active-extension-points",
            axum::routing::get(crate::api::aurora_admin::serve_active_theme_extension_points),
        )
        // v0.9 — login-splash branding + the resolved default theme for the
        // pre-auth login page. Unauthenticated (the page reads it before any
        // token exists); returns only the deployment-default theme id and the
        // operator-set branding URLs — all non-secret.
        .route(
            "/theme/login-branding",
            axum::routing::get(crate::api::aurora_admin::serve_login_branding),
        )
        // v0.9 (#329) — serve uploaded branding assets from <data>/branding/.
        // Public (the pre-auth login page fetches the logo/banner by URL).
        .route(
            "/branding/:filename",
            axum::routing::get(crate::api::aurora_admin::serve_branding_asset),
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
